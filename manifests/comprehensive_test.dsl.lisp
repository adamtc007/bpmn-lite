(workflow comprehensive-test-workflow
  (start-event :id start :next create-cbu)
  
  (service-task :id create-cbu :verb ob-poc:cbu.create :next type-decision)
  
  (business-rule-task :id type-decision :decision dmn-lite:cbu_type_routing :next type-gateway)
  
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next run-loop)
    (flow :condition (= @cbu-type "corporate") :next run-parallel)
    (flow :condition (= @cbu-type "trust")     :next run-single))
  
  (loop :id run-loop :ceiling 2 :body (
     (service-task :id loop-step :verb ob-poc:cbu.add-product :args (:product "loop-item") :next run-loop)
  ) :next end)
  
  (split-and :id run-parallel :join parallel-join
    (flow :next task-p1)
    (flow :next task-p2))
  (service-task :id task-p1 :verb ob-poc:cbu.add-product :args (:product "part1") :next parallel-join)
  (service-task :id task-p2 :verb ob-poc:cbu.add-product :args (:product "part2") :next parallel-join)
  (join-and :id parallel-join :split run-parallel :next end)
  
  (service-task :id run-single :verb ob-poc:instrument-matrix.attach :next end)
  
  (end-event :id end :status "Operational"))
